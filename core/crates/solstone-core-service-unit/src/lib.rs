// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure rendering primitives for Solstone launchd and systemd service units.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod env;
mod plist;
mod systemd;
mod windows_action;
mod windows_task;
mod windows_task_readback;

pub use env::build_service_environment;
pub use plist::render_launchd_plist;
pub use systemd::render_systemd_unit;
pub use windows_task::{WindowsTaskInput, render_windows_task_xml};

pub use windows_action::{
    WindowsServiceAction, decode_windows_task_arguments, encode_windows_task_arguments,
};

pub use windows_task_readback::{
    WindowsTaskDefinition, decode_windows_task_xml, encode_windows_task_xml, parse_windows_task_xml,
};
