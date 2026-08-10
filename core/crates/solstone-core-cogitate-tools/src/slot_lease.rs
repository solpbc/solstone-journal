// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

/// The outcome of reacquiring local inference capacity after a `sol` command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SlotReacquireError {
    Cancelled,
    Other(String),
}

/// Local-inference capacity that can be yielded while a journal command runs.
pub trait SlotLease {
    fn yield_slot(&mut self);
    fn reacquire(&mut self) -> Result<(), SlotReacquireError>;
    fn cancel_pending_reacquire(&mut self);
}

/// The default for providers that do not govern local inference capacity.
#[derive(Default)]
pub struct NoopSlotLease;

impl SlotLease for NoopSlotLease {
    fn yield_slot(&mut self) {}

    fn reacquire(&mut self) -> Result<(), SlotReacquireError> {
        Ok(())
    }

    fn cancel_pending_reacquire(&mut self) {}
}
