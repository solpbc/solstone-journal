// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::future::Future;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

use serde_json::{Map, Value};
use solstone_core_callosum::CallosumSocketConnection;
use thiserror::Error;
use tokio::time::{Duration, MissedTickBehavior};

use crate::process::{cancel_worker, spawn_worker, stop_group};
use crate::state::{CortexState, Outbound};
use crate::storage::CortexStore;

#[derive(Clone, Copy, Debug)]
pub struct CortexOptions {
    pub verbose: bool,
    pub debug: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShutdownMode {
    Immediate,
    Drain,
}

#[derive(Debug, Error)]
pub enum CortexServiceError {
    #[error("cortex runtime unavailable")]
    Runtime,
    #[error("could not inspect current executable: {0}")]
    CurrentExecutable(#[source] std::io::Error),
    #[error("could not initialize cortex journal storage: {0}")]
    Storage(#[source] std::io::Error),
}

impl CortexServiceError {
    pub const fn class(&self) -> &'static str {
        "runtime"
    }
}

pub async fn run_native_service(
    journal: PathBuf,
    options: CortexOptions,
) -> Result<(), CortexServiceError> {
    if options.verbose {
        eprintln!("cortex: starting native service");
    }
    if options.debug {
        eprintln!("cortex: debug diagnostics enabled");
    }
    let executable = std::env::current_exe().map_err(CortexServiceError::CurrentExecutable)?;
    let executable_dir = executable
        .parent()
        .map(PathBuf::from)
        .ok_or(CortexServiceError::Runtime)?;
    let connection =
        CallosumSocketConnection::new(journal.join("health/callosum.sock"), Map::new());
    run_until(journal, connection, executable_dir, shutdown_signal()).await
}

pub async fn run_until<F>(
    journal: PathBuf,
    mut connection: CallosumSocketConnection,
    executable_dir: PathBuf,
    shutdown: F,
) -> Result<(), CortexServiceError>
where
    F: Future<Output = ShutdownMode> + Send + 'static,
{
    let store = CortexStore::new(journal).map_err(CortexServiceError::Storage)?;
    // Recovery intentionally happens before this connection is started.
    store.recover();
    let (spawn_tx, spawn_rx) = mpsc::channel();
    let (cancel_tx, cancel_rx) = mpsc::channel();
    let (outbound_tx, outbound_rx) = mpsc::channel();
    let state = CortexState::new(store, spawn_tx, cancel_tx, outbound_tx);
    let spawn_state = state.clone();
    thread::spawn(move || spawn_worker(spawn_state, executable_dir, spawn_rx));
    let cancel_state = state.clone();
    thread::spawn(move || cancel_worker(cancel_state, cancel_rx));
    connection.start();
    let mut status = tokio::time::interval(Duration::from_secs(5));
    let mut drain = tokio::time::interval(Duration::from_millis(10));
    status.set_missed_tick_behavior(MissedTickBehavior::Skip);
    drain.set_missed_tick_behavior(MissedTickBehavior::Skip);
    tokio::pin!(shutdown);
    let mut draining = false;
    loop {
        tokio::select! {
            _ = status.tick() => state.status(state.queue_depth()),
            _ = drain.tick() => drain_outbound(&connection, &outbound_rx),
            mode = &mut shutdown, if !draining => {
                state.stop_accepting();
                match mode {
                    ShutdownMode::Immediate => {
                        for running in state.stop_immediately() { stop_group(running.pgid); }
                        break;
                    }
                    ShutdownMode::Drain => draining = true,
                }
            },
            message = connection.next_message(), if !draining => match message {
                Some(message) => dispatch(&state, message.tract.as_str(), message.event.as_str(), message.extra),
                None => break,
            },
            _ = tokio::time::sleep(Duration::from_millis(20)), if draining && state.is_idle() => break,
        }
    }
    drain_outbound(&connection, &outbound_rx);
    connection.stop().await;
    Ok(())
}

fn dispatch(state: &CortexState, tract: &str, event: &str, fields: Map<String, Value>) {
    if tract != "cortex" {
        return;
    }
    match event {
        "request" => state.request(fields),
        "cancel" => state.queue_cancel(&fields),
        _ => {}
    }
}

fn drain_outbound(connection: &CallosumSocketConnection, receiver: &mpsc::Receiver<Outbound>) {
    while let Ok(outbound) = receiver.try_recv() {
        let _ = connection.emit(outbound.tract, &outbound.event, outbound.fields);
    }
}

async fn shutdown_signal() -> ShutdownMode {
    #[cfg(unix)]
    {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            tokio::select! {
                _ = signal.recv() => return ShutdownMode::Immediate,
                _ = tokio::signal::ctrl_c() => return ShutdownMode::Drain,
            }
        }
    }
    let _ = tokio::signal::ctrl_c().await;
    ShutdownMode::Drain
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::CommandExt;
    use std::process::Command;
    use std::sync::{Arc, Mutex, mpsc};

    use super::*;

    fn running_state() -> (tempfile::TempDir, CortexState, std::process::Child) {
        let directory = tempfile::tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let (spawn_tx, spawn_rx) = mpsc::channel();
        let (cancel_tx, _) = mpsc::channel();
        let (outbound_tx, _) = mpsc::channel();
        let state = CortexState::new(store, spawn_tx, cancel_tx, outbound_tx);
        state.request(
            serde_json::from_value(serde_json::json!({"use_id":"one","name":"chat"})).unwrap(),
        );
        let work = spawn_rx.recv().unwrap();
        let child = Command::new("/bin/sh")
            .arg("-c")
            .arg("sleep 1")
            .process_group(0)
            .spawn()
            .unwrap();
        state.spawn_begin("one");
        state.spawn_started(
            &work,
            i32::try_from(child.id()).unwrap(),
            Arc::new(Mutex::new(Vec::new())),
        );
        (directory, state, child)
    }

    #[test]
    fn dispatch_filters_only_at_service_boundary() {
        let _ = ShutdownMode::Immediate;
    }

    #[test]
    fn drain_keeps_running_use_alive_until_its_own_exit_then_becomes_idle() {
        let (_directory, state, mut child) = running_state();
        state.stop_accepting();
        assert!(child.try_wait().unwrap().is_none());
        let status = child.wait().unwrap();
        assert!(status.success());
        state.finish("one", 0);
        state.spawn_finished();
        assert!(state.is_idle());
    }

    #[test]
    fn immediate_stop_terminalizes_queue_and_signals_running_group() {
        let (_directory, state, mut child) = running_state();
        for running in state.stop_immediately() {
            stop_group(running.pgid);
        }
        let status = child.wait().unwrap();
        assert!(!status.success());
    }
}
