// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

pub use super::server::ServerTestHooks;

#[doc(hidden)]
pub fn diagnostic_fixture(
    capacity: usize,
) -> (
    super::CallosumDiagnosticEmitter,
    tokio::sync::mpsc::Receiver<crate::CallosumEnvelope>,
) {
    super::connection::diagnostic_fixture(capacity)
}

use std::path::Path;

use serde_json::Map;

use super::CallosumRetrySource;
use super::connection::{CallosumSocketConnection, ConnectionCounters};

/// Build a real wire client at a counter boundary without exposing counter
/// mutation in production builds.
#[doc(hidden)]
pub fn connection_with_initial_counters(
    socket_path: impl AsRef<Path>,
    retry_source: Box<dyn CallosumRetrySource>,
    generation: u64,
    epoch: u64,
    attempt: u64,
    failures_since_success: u64,
    first_attempt: bool,
) -> CallosumSocketConnection {
    CallosumSocketConnection::with_retry_source_and_initial_counters(
        socket_path,
        Map::new(),
        1,
        retry_source,
        ConnectionCounters {
            generation,
            epoch,
            attempt,
            failures_since_success,
        },
        first_attempt,
    )
}

/// Consume the task handle after a test-forced terminal state.
#[doc(hidden)]
pub async fn join_terminated(connection: &mut CallosumSocketConnection) {
    connection.join_terminated_for_test().await;
}

/// Force only the optional capture constructor's failure, never real work.
#[doc(hidden)]
pub fn unavailable_brain_startup_capture(
    allocation_failure: bool,
) -> std::sync::Arc<super::BrainStartupCapture> {
    super::brain_startup::BrainStartupCapture::unavailable_for_test(allocation_failure)
}

#[doc(hidden)]
pub fn diagnostic_fixture_with_startup_capture(
    capacity: usize,
    capture: std::sync::Arc<super::BrainStartupCapture>,
) -> (
    super::CallosumDiagnosticEmitter,
    tokio::sync::mpsc::Receiver<crate::CallosumEnvelope>,
) {
    super::connection::diagnostic_fixture_with_startup_capture(capacity, capture)
}

/// Answer local-inference credential requests for `journal` the way its
/// supervisor does, until dropped. Integration tests that drive the bundled
/// local lane against a stub engine need it on platforms whose clients fetch
/// the engine's key over Callosum.
#[doc(hidden)]
pub struct LocalInferenceCredentialFixture {
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[doc(hidden)]
#[must_use]
pub fn serve_local_inference_credential(
    journal: &Path,
    port: u16,
    token: &[u8],
) -> LocalInferenceCredentialFixture {
    let socket = journal.join("health").join("callosum.sock");
    let snapshot =
        crate::LocalInferenceSnapshot::try_new(1, port, token).expect("fixture credential");
    let (ready, bound) = std::sync::mpsc::channel();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let thread = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("fixture runtime");
        runtime.block_on(async move {
            let server = super::CallosumSocketServer::bind(&socket)
                .await
                .expect("bind fixture callosum");
            server.set_local_inference_snapshot_source(move || {
                crate::LocalInferenceSnapshotOffer::Ready(snapshot.clone())
            });
            let _ = ready.send(());
            let _ = stopped.await;
            server.stop().await;
        });
    });
    bound.recv().expect("fixture callosum bound");
    LocalInferenceCredentialFixture {
        stop: Some(stop),
        thread: Some(thread),
    }
}

impl Drop for LocalInferenceCredentialFixture {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
