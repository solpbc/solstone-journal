// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io;
use std::ops::{Deref, DerefMut};
use std::process::{Child, ExitStatus};
use std::thread;
use std::time::{Duration, Instant};

use solstone_core_system::process::terminate;

const DROP_GRACE: Duration = Duration::from_secs(5);
#[allow(dead_code)]
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Own a supervisor process and clean up its complete process tree on every exit path.
pub(super) struct SupervisorGuard(Child);

impl SupervisorGuard {
    pub(super) fn new(child: Child) -> Self {
        Self(child)
    }

    /// Ask the supervisor to perform its normal shutdown and wait for its exit.
    ///
    /// A timeout leaves forced tree cleanup to `Drop`.
    #[allow(dead_code)]
    pub(super) fn shutdown_and_wait(&mut self, timeout: Duration) -> io::Result<ExitStatus> {
        if let Some(status) = self.0.try_wait()? {
            return Ok(status);
        }
        let pid = i32::try_from(self.0.id())
            .map_err(|_| io::Error::other("supervisor pid does not fit in i32"))?;
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(pid),
            nix::sys::signal::Signal::SIGTERM,
        )
        .or_else(|error| match error {
            nix::errno::Errno::ESRCH => Ok(()),
            _ => Err(error),
        })
        .map_err(io::Error::from)?;

        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.0.try_wait()? {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "supervisor did not exit after SIGTERM",
                ));
            }
            thread::sleep(POLL_INTERVAL);
        }
    }
}

impl Deref for SupervisorGuard {
    type Target = Child;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for SupervisorGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for SupervisorGuard {
    fn drop(&mut self) {
        match self.0.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => {}
            Err(error) => eprintln!("supervisor cleanup status check failed: {error}"),
        }
        if let Err(error) = terminate(&mut self.0, DROP_GRACE) {
            eprintln!("supervisor process-tree cleanup failed: {error}");
        }
        let _ = self.0.try_wait();
    }
}
