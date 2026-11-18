// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;
use std::sync::Arc;

use jiff::{Timestamp, ZonedDateTime};

/// Injectable clock: the current instant together with its local zone.
///
/// Day strings and local wall-time forms come from the local reading
/// (`datetime()`); epoch milliseconds and UTC timestamps come from the
/// instant, so neither is shifted by the local offset.
#[derive(Clone)]
pub struct Clock(Arc<dyn Fn() -> ZonedDateTime + Send + Sync>);

impl Clock {
    /// Now in the journal's owner zone, read afresh so a changed setting
    /// applies at once.
    pub fn owner(journal: impl Into<PathBuf>) -> Self {
        let journal = journal.into();
        Self(Arc::new(move || {
            Timestamp::now().to_zoned_datetime(solstone_core_journal_config::owner_zone(&journal))
        }))
    }

    pub fn new(now: impl Fn() -> ZonedDateTime + Send + Sync + 'static) -> Self {
        Self(Arc::new(now))
    }

    pub fn now(&self) -> ZonedDateTime {
        (self.0)()
    }
}
