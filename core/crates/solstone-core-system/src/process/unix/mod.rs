// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod authority;
mod descendants;
mod instance;
mod macos_proc;
mod pdeathsig;
mod spawn;
mod terminate;

pub use authority::{
    LaunchAuthority, launch, launch_command, launch_command_hosted, launch_managed,
    launch_managed_hosted, launch_managed_request, launch_managed_with, launch_with,
};
#[cfg(target_os = "linux")]
pub(crate) use instance::hold_while_instance_live;
#[cfg(target_os = "macos")]
pub(crate) use instance::macos_sweep_table;
pub use pdeathsig::apply_parent_death_kill;
pub use spawn::ManagedProcess;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) use terminate::signal_pid;
pub use terminate::{
    signal_exact_instance, terminate, terminate_descendants_exact, terminate_exact_instance,
};
