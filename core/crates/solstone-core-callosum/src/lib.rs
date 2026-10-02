// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Callosum wire envelopes and their durable per-segment event log.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod local_inference;
mod model;
mod oneshot;
mod reader;
mod registry;
#[cfg(any(test, windows))]
pub mod windows;
#[cfg(feature = "wire")]
mod wire;
mod writer;

pub use local_inference::LOCAL_INFERENCE_TOKEN_MAX;
#[cfg(feature = "wire")]
pub use local_inference::{
    LocalInferenceReadError, LocalInferenceSnapshot, LocalInferenceSnapshotOffer,
};
pub use model::{CallosumEnvelope, DeviceIngestEvent, DurableEvent, FileDescriptor, ReportedZone};
pub use oneshot::{CallosumOneShotError, CallosumOneShotSender};
pub use reader::{
    CallosumReadError, DeviceIngestReport, DurableEventsReport, read_device_ingest_events,
    read_durable_events, read_reported_zone,
};
pub use registry::callosum_registry;
#[cfg(feature = "full-tests")]
#[doc(hidden)]
pub use wire::one_shot_fixture as test_fixture;
#[cfg(all(feature = "wire", any(test, feature = "test-hooks")))]
#[doc(hidden)]
pub use wire::test_support;
#[cfg(feature = "wire")]
pub use wire::{
    CallosumConnectionPhase, CallosumGapReason, CallosumReceiveEvent, CallosumRetrySource,
    CallosumSocketConnection, CallosumSocketServer, CallosumSocketServerError,
    CallosumStoppedReason, TokioRetrySource, request_local_inference_snapshot,
    request_local_inference_snapshot_sync,
};
pub use writer::{CallosumWriteError, append_durable_event};
