// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native owner of the observer registry and sync-history files.

mod command;
mod service;
pub mod store;

pub use command::{ObserverCommand, parse_observer_args};
pub use service::{CREATE_RETIRED_MESSAGE, ObserverError, execute, system_now_ms};
