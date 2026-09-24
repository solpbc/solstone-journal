// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Linked-device push registration routes, durable registry, and notification sealing.
//!
//! This crate records linked-device push registrations and seals notification
//! envelopes. It does not send notifications.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod envelope;
mod model;
mod router;
mod store;
#[cfg(test)]
mod test_log;

pub use router::api_router;
