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
mod windows_task_reason;

pub use env::build_service_environment;
pub use plist::{launchd_plist_port, render_launchd_plist};
pub use systemd::{
    LAUNCHD_DEFAULT_EXIT_TIMEOUT_SECONDS, SERVICE_STOP_TIMEOUT_SECONDS, render_systemd_unit,
    systemd_unit_port,
};
pub use windows_task::{WindowsTaskInput, render_windows_task_xml};

pub use windows_action::{
    WindowsServiceAction, decode_windows_task_arguments, encode_windows_task_arguments,
};

pub use windows_task_readback::{
    WindowsTaskDefinition, decode_windows_task_xml, encode_windows_task_xml, parse_windows_task_xml,
};

pub use windows_task_reason::{WINDOWS_TASK_FAILURE_SCHEMA, windows_task_failure_reason};
