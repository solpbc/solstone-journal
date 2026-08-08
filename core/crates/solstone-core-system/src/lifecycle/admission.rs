// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(target_os = "linux")]
use std::fs::File;

#[cfg(target_os = "linux")]
use nix::errno::Errno;
#[cfg(target_os = "linux")]
use nix::fcntl::{Flock, FlockArg};

#[cfg(target_os = "linux")]
use super::LifecycleError;

/// Keeps the supervisor singleton flock alive for the lifecycle's lifetime.
pub struct SupervisorLease {
    #[cfg(target_os = "linux")]
    _lock: Flock<File>,
}

#[cfg(target_os = "linux")]
pub fn acquire(file: File) -> Result<SupervisorLease, LifecycleError> {
    match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
        Ok(lock) => Ok(SupervisorLease { _lock: lock }),
        Err((_file, error)) if is_contended(error) => Err(LifecycleError::AlreadyRunning),
        Err((_file, error)) => Err(LifecycleError::Nix(error)),
    }
}

#[cfg(target_os = "linux")]
fn is_contended(error: Errno) -> bool {
    error == Errno::EACCES || error == Errno::EAGAIN || error == Errno::EWOULDBLOCK
}
