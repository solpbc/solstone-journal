// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Linked-device push registration routes and their durable registry.
//!
//! Device tokens are retained locally for later delivery work, but this crate
//! deliberately does not implement a hosted relay or notification dispatch.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod model;
mod router;
mod store;

pub use router::api_router;
