// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Callosum wire envelopes and their durable per-segment event log.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod model;
mod oneshot;
mod reader;
mod registry;
#[cfg(any(test, windows))]
mod windows;
#[cfg(feature = "wire")]
mod wire;
mod writer;

pub use model::{CallosumEnvelope, DeviceIngestEvent, DurableEvent, FileDescriptor};
pub use oneshot::{CallosumOneShotError, CallosumOneShotSender};
pub use reader::{
    CallosumReadError, DeviceIngestReport, DurableEventsReport, read_device_ingest_events,
    read_durable_events,
};
pub use registry::callosum_registry;
#[cfg(all(feature = "wire", any(test, feature = "test-hooks")))]
#[doc(hidden)]
pub use wire::test_support;
#[cfg(feature = "wire")]
pub use wire::{
    CallosumConnectionPhase, CallosumGapReason, CallosumReceiveEvent, CallosumRetrySource,
    CallosumSocketConnection, CallosumSocketServer, CallosumSocketServerError,
    CallosumStoppedReason, TokioRetrySource,
};
pub use writer::{CallosumWriteError, append_durable_event};
