// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::os::fd::{AsFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use nix::fcntl::{FcntlArg, OFlag, fcntl};
use nix::sys::stat::{SFlag, fstat};
use nix::unistd::{Pid, getpgid, getpgrp, getppid, read};

use crate::queue::TaskQueue;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParentDeathReason {
    Eof,
    FdError,
    Orphaned,
}

pub fn wait_until_parent_gone(parent_fd: OwnedFd, poll_interval: Duration) -> ParentDeathReason {
    if parent_fd_is_usable(parent_fd.as_fd()) {
        loop {
            let mut buffer = [0_u8; 4096];
            match read(parent_fd.as_fd(), &mut buffer) {
                Ok(0) => return ParentDeathReason::Eof,
                Ok(_) => continue,
                Err(_) => return ParentDeathReason::FdError,
            }
        }
    }
    loop {
        if getppid() == Pid::from_raw(1) {
            return ParentDeathReason::Orphaned;
        }
        std::thread::sleep(poll_interval);
    }
}

fn parent_fd_is_usable(fd: std::os::fd::BorrowedFd<'_>) -> bool {
    let Ok(stat) = fstat(fd) else {
        return false;
    };
    let Ok(flags) = fcntl(fd, FcntlArg::F_GETFL) else {
        return false;
    };
    let access = OFlag::from_bits_truncate(flags) & OFlag::O_ACCMODE;
    SFlag::from_bits_truncate(stat.st_mode).contains(SFlag::S_IFIFO)
        && matches!(access, OFlag::O_RDONLY | OFlag::O_RDWR)
}

/// One-shot parent-death hard backstop. Callers supply process ids already owned
/// by the supervisor and inject sleep/exit for deterministic tests.
pub struct ParentDeathBackstop {
    fired: AtomicBool,
}

impl Default for ParentDeathBackstop {
    fn default() -> Self {
        Self {
            fired: AtomicBool::new(false),
        }
    }
}

impl ParentDeathBackstop {
    pub fn enforce(
        &self,
        ceiling: Duration,
        managed_pids: &[u32],
        queue: Option<&TaskQueue>,
        sleep: impl FnOnce(Duration),
        self_term: impl FnOnce(),
        exit: impl FnOnce(i32),
    ) {
        if self.fired.swap(true, Ordering::SeqCst) {
            return;
        }
        self_term();
        sleep(ceiling);
        let mut pids = managed_pids.to_vec();
        if let Some(queue) = queue {
            pids.extend(
                queue
                    .active_process_handles()
                    .into_iter()
                    .map(|handle| handle.pid()),
            );
        }
        let own_pid = std::process::id() as i32;
        let own_pgid = getpgrp().as_raw();
        for pid in pids {
            let Ok(pgid) = getpgid(Some(Pid::from_raw(pid as i32))) else {
                continue;
            };
            let pgid = pgid.as_raw();
            if pgid <= 1 || pgid == own_pid || pgid == own_pgid {
                continue;
            }
            let _ =
                nix::sys::signal::killpg(Pid::from_raw(pgid), nix::sys::signal::Signal::SIGKILL);
        }
        exit(1);
    }
}

#[cfg(test)]
mod tests {
    use std::os::fd::AsFd;

    use nix::fcntl::{OFlag, open};
    use nix::sys::stat::Mode;

    use super::parent_fd_is_usable;

    #[test]
    fn regular_file_is_not_a_parent_death_pipe() {
        let fd = open("/dev/null", OFlag::O_RDONLY, Mode::empty()).expect("open regular file");
        assert!(!parent_fd_is_usable(fd.as_fd()));
    }
}
