// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};

use serde_json::Map;
use solstone_core_callosum::CallosumSocketConnection;
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
}
impl NativeServiceError {
    pub const fn class(self) -> &'static str {
        "runtime"
    }
}

pub fn run_native_service(
    journal: PathBuf,
    options: SenseOptions,
) -> Result<(), NativeServiceError> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("sense-service")
        .build()
        .map_err(|_| NativeServiceError::Runtime)?
        .block_on(run(journal, options));
    Ok(())
}

async fn run(journal: PathBuf, options: SenseOptions) {
    let connection =
        CallosumSocketConnection::new(journal.join("health/callosum.sock"), Map::new());
    let (outbound, receiver) = mpsc::channel::<Outbound>();
    let dispatcher = Arc::new(SenseDispatcher::new(
        journal,
        options.verbose,
        options.debug,
        outbound,
    ));
    run_until(connection, dispatcher, receiver, shutdown_signal()).await;
}

/// Drives the service loop until the supplied shutdown future resolves.
///
/// The production signal path and socket integration tests share this exact
/// lifecycle so worker output is always drained after bounded termination.
pub async fn run_until<F>(
    mut connection: CallosumSocketConnection,
    dispatcher: Arc<SenseDispatcher>,
    receiver: mpsc::Receiver<Outbound>,
    shutdown: F,
) where
    F: Future<Output = ()> + Send + 'static,
{
    connection.start();
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    let mut outbound_interval = tokio::time::interval(Duration::from_millis(10));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    outbound_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = interval.tick() => dispatcher.status(),
            _ = outbound_interval.tick() => drain(&connection, &receiver),
            _ = &mut shutdown => break,
            message = connection.next_message() => {
                if let Some(message) = message {
                    dispatcher.handle(&message);
                } else {
                    break;
                }
            }
        }
    }
    let stopping_dispatcher = Arc::clone(&dispatcher);
    let _ = tokio::task::spawn_blocking(move || stopping_dispatcher.stop_and_wait()).await;
    drain(&connection, &receiver);
    connection.stop().await;
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
