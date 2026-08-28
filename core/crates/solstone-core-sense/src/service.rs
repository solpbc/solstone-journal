// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};

use serde_json::Map;
use solstone_core_callosum::CallosumSocketConnection;
use solstone_core_system::lifecycle::{
    HostedServiceParentRuntime, HostedServiceShutdownEvidence, ParentLossReason,
};
use thiserror::Error;
use tokio::time::{Duration, MissedTickBehavior};

use crate::dispatch::{Outbound, SenseDispatcher};

#[derive(Clone, Copy, Debug)]
pub struct SenseOptions {
    pub verbose: bool,
    pub debug: bool,
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum NativeServiceError {
    #[error("service runtime unavailable")]
    Runtime,
    #[error("hosted parent-loss coordination failed")]
    ParentLoss,
}
impl NativeServiceError {
    pub const fn class(self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::ParentLoss => "parent-loss",
        }
    }
}

pub fn run_native_service(
    journal: PathBuf,
    options: SenseOptions,
) -> Result<(), NativeServiceError> {
    run_native_service_with_hosted_parent(journal, options, None)
}

/// Run Sense with an optional birth-admitted hosted parent lifetime.
pub fn run_native_service_with_hosted_parent(
    journal: PathBuf,
    options: SenseOptions,
    hosted_parent: Option<Arc<HostedServiceParentRuntime>>,
) -> Result<(), NativeServiceError> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("sense-service")
        .build()
        .map_err(|_| NativeServiceError::Runtime)?
        .block_on(run(journal, options, hosted_parent))
}

async fn run(
    journal: PathBuf,
    options: SenseOptions,
    hosted_parent: Option<Arc<HostedServiceParentRuntime>>,
) -> Result<(), NativeServiceError> {
    let connection =
        CallosumSocketConnection::new(journal.join("health/callosum.sock"), Map::new());
    let (outbound, receiver) = mpsc::channel::<Outbound>();
    let dispatcher = Arc::new(SenseDispatcher::new_with_hosted_parent(
        journal,
        options.verbose,
        options.debug,
        outbound,
        hosted_parent.clone(),
    ));
    let outcome = run_until_with_shutdown_status(
        connection,
        dispatcher,
        receiver,
        shutdown_with_hosted_parent(hosted_parent.clone()),
    )
    .await;
    let parent_loss = outcome.shutdown.flatten();
    if let Some(parent) = hosted_parent {
        let reason = parent_loss.or_else(|| {
            parent
                .retire_expected_requested()
                .then_some(ParentLossReason::ExitedOrReused)
        });
        if reason.is_some() {
            // `connection.stop` is infallible and always runs before this point.
            // Worker joins are independently observed so a panic cannot claim a
            // completed service runner in parent-loss coordination.
            parent
                .finish_parent_loss(HostedServiceShutdownEvidence {
                    listener_stopped: true,
                    service_runner_stopped: outcome.service_runner_stopped,
                    // Sense has no separate health artifact to withdraw.
                    operational_artifacts_cleaned: true,
                })
                .map_err(|_| NativeServiceError::ParentLoss)?;
        }
    }
    Ok(())
}

/// Drives the service loop until the supplied shutdown future resolves.
///
/// The production signal path and socket integration tests share this exact
/// lifecycle so worker output is always drained after bounded termination.
pub async fn run_until<F, T>(
    connection: CallosumSocketConnection,
    dispatcher: Arc<SenseDispatcher>,
    receiver: mpsc::Receiver<Outbound>,
    shutdown: F,
) -> Option<T>
where
    F: Future<Output = T> + Send + 'static,
{
    run_until_with_shutdown_status(connection, dispatcher, receiver, shutdown)
        .await
        .shutdown
}

struct ServiceLoopOutcome<T> {
    shutdown: Option<T>,
    service_runner_stopped: bool,
}

async fn run_until_with_shutdown_status<F, T>(
    mut connection: CallosumSocketConnection,
    dispatcher: Arc<SenseDispatcher>,
    receiver: mpsc::Receiver<Outbound>,
    shutdown: F,
) -> ServiceLoopOutcome<T>
where
    F: Future<Output = T> + Send + 'static,
{
    connection.start();
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    let mut outbound_interval = tokio::time::interval(Duration::from_millis(10));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    outbound_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    tokio::pin!(shutdown);
    let shutdown_outcome = loop {
        tokio::select! {
            _ = interval.tick() => dispatcher.status(),
            _ = outbound_interval.tick() => drain(&connection, &receiver),
            outcome = &mut shutdown => break Some(outcome),
            message = connection.next_message() => {
                if let Some(message) = message {
                    dispatcher.handle(&message);
                } else {
                    break None;
                }
            }
        }
    };
    let stopping_dispatcher = Arc::clone(&dispatcher);
    let service_runner_stopped =
        tokio::task::spawn_blocking(move || stopping_dispatcher.stop_and_wait())
            .await
            .unwrap_or(false);
    drain(&connection, &receiver);
    connection.stop().await;
    ServiceLoopOutcome {
        shutdown: shutdown_outcome,
        service_runner_stopped,
    }
}

async fn shutdown_with_hosted_parent(
    hosted_parent: Option<Arc<HostedServiceParentRuntime>>,
) -> Option<ParentLossReason> {
    let Some(parent) = hosted_parent else {
        shutdown_signal().await;
        return None;
    };
    tokio::select! {
        _ = shutdown_signal() => None,
        reason = parent.await_parent_loss() => Some(reason),
    }
}

fn drain(connection: &CallosumSocketConnection, receiver: &mpsc::Receiver<Outbound>) {
    while let Ok(event) = receiver.try_recv() {
        let _ = connection.emit(event.tract, event.event, event.fields);
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = signal.recv() => {} };
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}
