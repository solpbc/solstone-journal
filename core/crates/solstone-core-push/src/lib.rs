// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Linked-device push registration routes, durable registry, notification sealing, and test delivery.
//!
//! This crate records linked-device push registrations, seals notification
//! envelopes, and executes push test deliveries to registered devices
//! through the hosted relay (for iOS) or direct Web Push (for Android).

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod endpoint;
pub mod envelope;
mod model;
mod relay;
mod router;
mod store;
#[cfg(test)]
mod test_log;
mod vapid;
mod web_push;

pub use router::api_router;
pub use store::{PushStoreError, remove_cid_registrations};
