// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShutdownReport {
    pub phases: Vec<ShutdownPhase>,
    pub disposition: ShutdownDisposition,
    pub forced_phase: Option<ShutdownPhase>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ShutdownDisposition {
    #[default]
    Orderly,
    ForcedAfterGraceTimeout,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArtifactClearOutcome {
    Cleared,
    Skipped,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShutdownOutcome {
    pub report: ShutdownReport,
    pub readiness: ArtifactClearOutcome,
    pub self_heartbeat: ArtifactClearOutcome,
    pub identity: ArtifactClearOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownRegime {
    AppSupervised,
    Standard,
}

/// Adapter for host-specific work; lifecycle owns the ordering witness.
pub trait ShutdownDriver {
    fn reap_managed(&mut self, cap: Duration) -> ShutdownDisposition;
    fn drain_tasks(&mut self, cap: Duration) -> ShutdownDisposition;
    fn stop_children(&mut self, cap: Option<Duration>) -> ShutdownDisposition;
    fn join_bus(&mut self, cap: Duration) -> ShutdownDisposition;
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
    record_disposition(
        &mut report,
        driver.reap_managed(reap),
        ShutdownPhase::ReapManagedCompleted,
    );
    report.phases.push(ShutdownPhase::ReapManagedCompleted);
    report.phases.push(ShutdownPhase::DrainTasksStarted);
    record_disposition(
        &mut report,
        driver.drain_tasks(drain),
        ShutdownPhase::DrainTasksCompleted,
    );
    report.phases.push(ShutdownPhase::DrainTasksCompleted);
    report.phases.push(ShutdownPhase::StopChildrenStarted);
    record_disposition(
        &mut report,
        driver.stop_children(children),
        ShutdownPhase::StopChildrenCompleted,
    );
    report.phases.push(ShutdownPhase::StopChildrenCompleted);
    report.phases.push(ShutdownPhase::JoinBusStarted);
    record_disposition(
        &mut report,
        driver.join_bus(bus),
        ShutdownPhase::JoinBusCompleted,
    );
    report.phases.push(ShutdownPhase::JoinBusCompleted);
    report
}

fn record_disposition(
    report: &mut ShutdownReport,
    disposition: ShutdownDisposition,
    phase: ShutdownPhase,
) {
    if matches!(disposition, ShutdownDisposition::ForcedAfterGraceTimeout)
        && matches!(report.disposition, ShutdownDisposition::Orderly)
    {
        report.disposition = disposition;
        report.forced_phase = Some(phase);
    }
}
