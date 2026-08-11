// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::fs::File;

#[cfg(any(target_os = "linux", target_os = "macos"))]
use nix::errno::Errno;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use nix::fcntl::{Flock, FlockArg};

#[cfg(any(target_os = "linux", target_os = "macos"))]
use super::LifecycleError;

/// Keeps the supervisor singleton flock alive for the lifecycle's lifetime.
pub struct SupervisorLease {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    _lock: Flock<File>,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn acquire(file: File) -> Result<SupervisorLease, LifecycleError> {
    match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
        Ok(lock) => Ok(SupervisorLease { _lock: lock }),
        Err((_file, error)) if is_contended(error) => Err(LifecycleError::AlreadyRunning),
        Err((_file, error)) => Err(LifecycleError::Nix(error)),
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn is_contended(error: Errno) -> bool {
    error == Errno::EACCES || error == Errno::EAGAIN || error == Errno::EWOULDBLOCK
}
