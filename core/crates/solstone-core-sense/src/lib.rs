// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native event-mode observation dispatcher.

pub mod batch;
pub mod beacon;
pub mod config;
pub mod dispatch;
pub mod events;
pub mod memory;
pub mod registry;
pub mod service;
pub mod work;

pub use dispatch::SenseDispatcher;
pub use service::{
    NativeServiceError, SenseOptions, run_native_service, run_native_service_with_hosted_parent,
};
