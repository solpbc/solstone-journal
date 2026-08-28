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
    #[error("hosted parent-loss handoff failed")]
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
    let dispatcher = Arc::new(SenseDispatcher::new(
        journal,
        options.verbose,
        options.debug,
        outbound,
    ));
    let parent_loss = run_until(
        connection,
        dispatcher,
        receiver,
        shutdown_with_hosted_parent(hosted_parent.clone()),
    )
    .await
    .flatten();
    if let (Some(parent), Some(reason)) = (hosted_parent, parent_loss) {
        // `run_until` reaches this point only after `stop_and_wait` joined the
        // dispatcher workers and `connection.stop` sent its infallible,
        // internally bounded connection shutdown. `stop_and_wait` has no
        // service-wide deadline, so this records observed completion rather
        // than claiming the generic parent-loss budget enforced it.
        parent
            .finish_parent_loss(
                reason,
                HostedServiceShutdownEvidence {
                    listener_stopped: true,
                    service_runner_stopped: true,
                    // Sense has no separate health artifact to withdraw.
                    operational_artifacts_cleaned: true,
                },
            )
            .map_err(|_| NativeServiceError::ParentLoss)?;
    }
    Ok(())
}

/// Drives the service loop until the supplied shutdown future resolves.
///
/// The production signal path and socket integration tests share this exact
/// lifecycle so worker output is always drained after bounded termination.
pub async fn run_until<F, T>(
    mut connection: CallosumSocketConnection,
    dispatcher: Arc<SenseDispatcher>,
    receiver: mpsc::Receiver<Outbound>,
    shutdown: F,
) -> Option<T>
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
    let _ = tokio::task::spawn_blocking(move || stopping_dispatcher.stop_and_wait()).await;
    drain(&connection, &receiver);
    connection.stop().await;
    shutdown_outcome
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
