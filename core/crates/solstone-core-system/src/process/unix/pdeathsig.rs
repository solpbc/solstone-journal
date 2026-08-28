// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

/// Install a Linux parent-death SIGKILL on `command` so a direct child cannot
/// outlive a SIGKILL of this process. No-op on non-Linux.
pub fn apply_parent_death_kill(command: &mut std::process::Command) {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;

        use nix::sys::prctl::set_pdeathsig;
        use nix::sys::signal::{self, Signal};
        use nix::unistd::{Pid, getppid};

        let expected_ppid = Pid::from_raw(i32::try_from(std::process::id()).unwrap_or(i32::MAX));
        // SAFETY: the closure runs between fork and exec and only calls
        // async-signal-safe functions (prctl, getppid, kill).
        #[allow(unsafe_code)]
        unsafe {
            command.pre_exec(move || {
                set_pdeathsig(Signal::SIGKILL)
                    .map_err(|errno| std::io::Error::from_raw_os_error(errno as i32))?;
                if getppid() != expected_ppid {
                    let _ = signal::kill(Pid::this(), Signal::SIGKILL);
                    return Err(std::io::Error::from_raw_os_error(
                        nix::errno::Errno::ESRCH as i32,
                    ));
                }
                Ok(())
            });
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = command;
    }
}
