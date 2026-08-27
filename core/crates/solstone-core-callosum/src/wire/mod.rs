// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod connection;
mod frame;
mod framing;
mod server;

pub use connection::{
    CallosumConnectionPhase, CallosumGapReason, CallosumReceiveEvent, CallosumRetrySource,
    CallosumSocketConnection, CallosumStoppedReason, TokioRetrySource,
};
pub use server::{CallosumSocketServer, CallosumSocketServerError};

#[cfg(any(test, feature = "test-hooks"))]
pub mod test_support;

pub(crate) const CLIENT_RECONNECT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);
pub(crate) const CLIENT_SEND_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(100);
pub(crate) const CLIENT_OUTBOUND_CAPACITY: usize = 1_000;
pub(crate) const CLIENT_INBOUND_CAPACITY: usize = 1_000;
pub(crate) const CLIENT_STOP_JOIN_TIMEOUT: std::time::Duration =
    std::time::Duration::from_millis(500);
pub(crate) const SERVER_BROADCAST_CAPACITY: usize = 10_000;
// Per-client queues stay small so one slow peer cannot consume the global budget.
pub(crate) const SERVER_CLIENT_OUTBOUND_CAPACITY: usize = 64;
pub(crate) const SERVER_SEND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
pub(crate) const SERVER_STOP_JOIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);
pub(crate) const READ_BUFFER_CAPACITY: usize = 4_096;

#[cfg(all(test, unix))]
mod tests;
