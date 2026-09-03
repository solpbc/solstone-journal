// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure rendering primitives for Solstone launchd and systemd service units.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod env;
mod plist;
mod systemd;

pub use env::build_service_environment;
pub use plist::render_launchd_plist;
pub use systemd::render_systemd_unit;
