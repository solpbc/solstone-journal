// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Darwin direct-parent exit observation backed by `kqueue(2)`.

use nix::errno::Errno;
use nix::sys::event::{EvFlags, EventFilter, FilterFlag, KEvent, Kqueue};
use thiserror::Error;

use crate::process::ProcessInstance;

/// A registered `EVFILT_PROC` exit watch for one birth-admitted parent PID.
pub struct DarwinParentExitWatcher {
    queue: Kqueue,
    parent_pid: u32,
}

/// Failure to register or wait on Darwin's kernel parent-exit source.
#[derive(Debug, Error)]
pub enum DarwinParentWatchError {
    #[error("could not create Darwin parent-exit kqueue: {0}")]
    CreateKqueue(Errno),
    #[error("could not register Darwin parent-exit filter: {0}")]
    RegisterExitFilter(Errno),
    #[error("could not wait for Darwin parent-exit filter: {0}")]
    Wait(Errno),
}

impl DarwinParentExitWatcher {
    /// Register the exit filter before the caller's final parent recheck.
    pub fn register(parent: ProcessInstance) -> Result<Self, DarwinParentWatchError> {
        let queue = Kqueue::new().map_err(DarwinParentWatchError::CreateKqueue)?;
        let registration = KEvent::new(
            parent.pid as usize,
            EventFilter::EVFILT_PROC,
            EvFlags::EV_ADD | EvFlags::EV_ENABLE | EvFlags::EV_CLEAR | EvFlags::EV_RECEIPT,
            FilterFlag::NOTE_EXIT,
            0,
            0,
        );
        let mut receipts = [empty_event()];
        let count = queue
            .kevent(&[registration], &mut receipts, None)
            .map_err(DarwinParentWatchError::RegisterExitFilter)?;
        if count != 1 || receipts[0].data() != 0 {
            return Err(DarwinParentWatchError::RegisterExitFilter(Errno::from_raw(
                receipts[0].data() as i32,
            )));
        }
        Ok(Self {
            queue,
            parent_pid: parent.pid,
        })
    }

    /// Block until the kernel reports this exact watched parent exited.
    pub fn wait_for_exit(&self) -> Result<(), DarwinParentWatchError> {
        let mut events = [empty_event()];
        let count = self
            .queue
            .kevent(&[], &mut events, None)
            .map_err(DarwinParentWatchError::Wait)?;
        let event = events[0];
        if count != 1
            || event.ident() != self.parent_pid as usize
            || event.filter().map_err(DarwinParentWatchError::Wait)? != EventFilter::EVFILT_PROC
            || !event.fflags().contains(FilterFlag::NOTE_EXIT)
        {
            return Err(DarwinParentWatchError::Wait(Errno::EINVAL));
        }
        if event.flags().contains(EvFlags::EV_ERROR) {
            return Err(DarwinParentWatchError::Wait(Errno::from_raw(
                event.data() as i32
            )));
        }
        Ok(())
    }
}

fn empty_event() -> KEvent {
    KEvent::new(
        0,
        EventFilter::EVFILT_PROC,
        EvFlags::empty(),
        FilterFlag::empty(),
        0,
        0,
    )
}
