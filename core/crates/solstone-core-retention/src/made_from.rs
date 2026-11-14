// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What was made from recordings, reached when they leave or come back.
//!
//! The trash moves a recording; what the journal made from it (the activities
//! over it, the day's briefing, the week's page) lives in stores this crate
//! can't see. Every kind that moves a recording takes a [`MadeFrom`] when it
//! is built, so none can move one without reaching what was made from it.

use std::path::Path;

use solstone_core_trash::{Change, Item};

use crate::receipt::Target;

/// What was made from recordings.
pub trait MadeFrom {
    /// The item's recordings `segments` just left (`Detached`) or came back
    /// (`Attached`), under the hold the trash took for them. Runs again, with
    /// the same item, when a stopped change is finished by the sweep, so it
    /// must be safe to repeat.
    fn recordings_changed(
        &self,
        journal: &Path,
        item: &Item,
        change: Change,
        segments: &[Target],
    ) -> Result<(), String>;
}

/// For a test or a tool that moves recordings with nothing made from them.
pub struct NothingMade;

impl MadeFrom for NothingMade {
    fn recordings_changed(
        &self,
        _journal: &Path,
        _item: &Item,
        _change: Change,
        _segments: &[Target],
    ) -> Result<(), String> {
        Ok(())
    }
}
