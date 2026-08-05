// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Callosum wire envelopes and their durable per-segment event log.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod model;
mod reader;
mod registry;
mod writer;

pub use model::{CallosumEnvelope, DeviceIngestEvent, DurableEvent, FileDescriptor};
pub use reader::{
    CallosumReadError, DeviceIngestReport, DurableEventsReport, read_device_ingest_events,
    read_durable_events,
};
pub use registry::callosum_registry;
pub use writer::{CallosumWriteError, append_durable_event};
