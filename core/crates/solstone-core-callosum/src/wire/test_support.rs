// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

pub use super::server::ServerTestHooks;

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
