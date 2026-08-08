// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownPhase {
    ReapManagedStarted,
    ReapManagedCompleted,
    DrainTasksStarted,
    DrainTasksCompleted,
    StopChildrenStarted,
    StopChildrenCompleted,
    JoinBusStarted,
    JoinBusCompleted,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ShutdownReport {
    pub phases: Vec<ShutdownPhase>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownRegime {
    AppSupervised,
    Standard,
}

/// Adapter for host-specific work; lifecycle owns the ordering witness.
pub trait ShutdownDriver {
    fn reap_managed(&mut self, cap: Duration);
    fn drain_tasks(&mut self, cap: Duration);
    fn stop_children(&mut self, cap: Option<Duration>);
    fn join_bus(&mut self, cap: Duration);
}

const APP_SUPERVISED_REAP_TIMEOUT: Duration = Duration::from_secs(3);
const APP_SUPERVISED_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
const APP_SUPERVISED_CHILD_STOP_TIMEOUT: Duration = Duration::from_secs(2);
const APP_SUPERVISED_BUS_JOIN_TIMEOUT: Duration = Duration::from_secs(2);
const APP_SUPERVISED_SHUTDOWN_CEILING: Duration = Duration::from_secs(10);
const _: () = assert!(
    APP_SUPERVISED_REAP_TIMEOUT.as_secs()
        + APP_SUPERVISED_DRAIN_TIMEOUT.as_secs()
        + APP_SUPERVISED_CHILD_STOP_TIMEOUT.as_secs()
        + APP_SUPERVISED_BUS_JOIN_TIMEOUT.as_secs()
        < APP_SUPERVISED_SHUTDOWN_CEILING.as_secs()
);

pub fn shutdown(driver: &mut dyn ShutdownDriver, regime: ShutdownRegime) -> ShutdownReport {
    match regime {
        ShutdownRegime::AppSupervised => shutdown_app_supervised(driver),
        ShutdownRegime::Standard => shutdown_standard(driver),
    }
}

fn shutdown_app_supervised(driver: &mut dyn ShutdownDriver) -> ShutdownReport {
    run_shutdown(
        driver,
        APP_SUPERVISED_REAP_TIMEOUT,
        APP_SUPERVISED_DRAIN_TIMEOUT,
        Some(APP_SUPERVISED_CHILD_STOP_TIMEOUT),
        APP_SUPERVISED_BUS_JOIN_TIMEOUT,
    )
}

fn shutdown_standard(driver: &mut dyn ShutdownDriver) -> ShutdownReport {
    run_shutdown(
        driver,
        Duration::from_secs(3),
        Duration::from_secs(10),
        None,
        Duration::from_secs(5),
    )
}

fn run_shutdown(
    driver: &mut dyn ShutdownDriver,
    reap: Duration,
    drain: Duration,
    children: Option<Duration>,
    bus: Duration,
) -> ShutdownReport {
    let mut report = ShutdownReport::default();
    report.phases.push(ShutdownPhase::ReapManagedStarted);
    driver.reap_managed(reap);
    report.phases.push(ShutdownPhase::ReapManagedCompleted);
    report.phases.push(ShutdownPhase::DrainTasksStarted);
    driver.drain_tasks(drain);
    report.phases.push(ShutdownPhase::DrainTasksCompleted);
    report.phases.push(ShutdownPhase::StopChildrenStarted);
    driver.stop_children(children);
    report.phases.push(ShutdownPhase::StopChildrenCompleted);
    report.phases.push(ShutdownPhase::JoinBusStarted);
    driver.join_bus(bus);
    report.phases.push(ShutdownPhase::JoinBusCompleted);
    report
}
